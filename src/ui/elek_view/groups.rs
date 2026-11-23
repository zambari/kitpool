//! Sample groups: named runs of adjacent sample slots in one bank that move
//! as one unit. Only this app knows them (the device has no such thing);
//! they're saved per unit by +Drive hash (`state::PoolGroup`), so a project
//! with the same samples gets its groups back.
//!
//! Rules: a group is a packed run of slots inside one bank. It may hold
//! empty slots (a *container*: places waiting for samples). Dropping slots
//! into a group fills its empty slots below the drop line first, else adds
//! them and pushes what follows down as far as the next free slot; dragging
//! one out closes the group up and pulls up what follows, as far as the
//! next free slot, and a container then takes free slots next to it back
//! up to its old size. An empty slot in a group is never "free" for a push
//! from outside it: it moves with its group. Every slot move is worked
//! out first (`ElekState::plan`), so the pointer note says what will happen
//! and a move that would break a group is refused before it happens.

use super::*;
pub use crate::state::PoolGroup;
use std::ops::Range;

#[derive(Clone, PartialEq)]
pub struct Group {
    pub name: String,
    /// Pool entries (as read) in the group, top to bottom. Always adjacent
    /// slots in one bank.
    pub members: Vec<u16>,
}

/// What a slot drop does, before it happens.
#[derive(Clone, Copy)]
pub(super) enum SlotDrop {
    /// The selection lands at `top`, pushing what's there down to the next
    /// empty slot.
    Shift(usize),
    /// The selection joins group `g` before its member `before` (`None`: at
    /// its end). `line` is the row boundary, for the feedback.
    Into { g: usize, before: Option<u16>, line: usize },
    /// All of group `g` lands at `top`, pushing down like `Shift`.
    Group { g: usize, top: usize },
    /// ⌥ with a group: its second half lands at `top` as a new group of the
    /// same name, the first half stays (`plan_split`).
    Split { g: usize, top: usize },
    /// ⌥: the selection swaps in place.
    Place(usize),
}

/// How hard a group reorder (`ElekState::plan_reorder`) may shuffle its
/// bank, gentlest first. Each level allows the ones before it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Shuffle {
    /// Only into free slots already there: nothing else moves.
    Fit,
    /// Push neighbours aside, each only as far as the next free slot.
    Push,
    /// Slide everything between the group and its new spot over by the
    /// group's length. Always possible inside a bank.
    Rotate,
}

/// A worked-out drop: the new order and groups, and what to say about it.
pub(super) struct Plan {
    pub order: Vec<u16>,
    pub groups: Vec<Group>,
    pub note: String,
}

/// `pos[old] = position now`.
pub(super) fn positions(order: &[u16]) -> Vec<usize> {
    let mut pos = vec![0; order.len()];
    for (i, &o) in order.iter().enumerate() {
        pos[o as usize] = i;
    }
    pos
}

/// The `group_hues` key for groups named `name`: names match ignoring case
/// and surrounding spaces, as everywhere else groups compare names.
pub fn hue_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn bank_end(p: usize, n: usize) -> usize {
    ((p / BANK + 1) * BANK).min(n)
}

impl ElekState {
    /// Each group's slots now, by index.
    /// The colour of groups named `name`: the device accent (`bright` when
    /// hot) turned by its hue offset.
    pub(super) fn group_color(&self, name: &str, bright: bool) -> egui::Color32 {
        let base = if bright { theme::DEVICE_ACCENT_BRIGHT } else { theme::DEVICE_ACCENT };
        theme::hue_shift(base, self.group_hues.get(&hue_key(name)).copied().unwrap_or(0.0))
    }

    pub(super) fn group_spans(&self) -> Vec<Range<usize>> {
        let pos = positions(&self.order);
        self.groups.iter().map(|g| span_of(g, &pos)).collect()
    }

    /// The group holding slot `slot`, if any.
    pub(super) fn group_at(&self, slot: usize) -> Option<usize> {
        let id = *self.order.get(slot)?;
        self.groups.iter().position(|g| g.members.contains(&id))
    }

    /// Slots `pick` can become a group: adjacent, in one bank, none grouped
    /// yet. Empty slots are fine (a container). `Err` says why not.
    pub(super) fn groupable(&self, pick: &[usize]) -> Result<(), &'static str> {
        let (Some(&a), Some(&b)) = (pick.iter().min(), pick.iter().max()) else { return Err("Nothing selected") };
        if a == 0 {
            return Err("Slot 000 is \"off\" and can't be grouped");
        }
        if b - a + 1 != pick.len() {
            return Err("Select adjacent slots (a continuous range)");
        }
        if a / BANK != b / BANK {
            return Err("A group stays inside one bank");
        }
        if pick.iter().any(|&s| self.group_at(s).is_some()) {
            return Err("Some of these slots are already in a group");
        }
        Ok(())
    }

    /// Why a group can't be called `name` in `bank`: another group of that
    /// name already sits in the bank. (Same names in other banks are fine.)
    fn name_taken(&self, name: &str, bank: usize, except: Option<usize>) -> Option<String> {
        let want = name.trim().to_lowercase();
        let spans = self.group_spans();
        let taken = self
            .groups
            .iter()
            .enumerate()
            .any(|(i, g)| Some(i) != except && spans[i].start / BANK == bank && g.name.trim().to_lowercase() == want);
        taken.then(|| format!("A group named \"{}\" already exists in bank {}", name.trim(), (b'A' + bank as u8) as char))
    }

    pub(super) fn new_group(&mut self, pick: &[usize], name: String) {
        if let Err(why) = self.groupable(pick) {
            self.status = why.into();
            return;
        }
        if let Some(&first) = pick.iter().min() {
            if let Some(why) = self.name_taken(&name, first / BANK, None) {
                self.status = why;
                return;
            }
        }
        let mut pick = pick.to_vec();
        pick.sort_unstable();
        let (a, b) = (pick[0], pick[pick.len() - 1]);
        let mut order = self.order.clone();
        let members: Vec<u16> = if pick.iter().all(|&s| self.free(self.order[s])) {
            // Only empty slots: a container, its places stay where they are.
            pick.iter().map(|&s| self.order[s]).collect()
        } else {
            // A group with samples is always packed: its empty slots within
            // the range move to just after it (other slots don't move).
            let (filled, empty): (Vec<u16>, Vec<u16>) =
                self.order[a..=b].iter().copied().partition(|&id| !self.free(id));
            let mut run = filled.clone();
            run.extend(empty);
            order.splice(a..=b, run);
            filled
        };
        let count = members.len();
        let mut groups = self.groups.clone();
        groups.push(Group { name: name.clone(), members });
        self.commit(order, groups);
        self.status = format!("Grouped {count} slot(s) as \"{name}\"");
    }

    pub(super) fn rename_group(&mut self, g: usize, name: String) {
        if let Some(span) = self.group_spans().get(g).cloned() {
            if let Some(why) = self.name_taken(&name, span.start / BANK, Some(g)) {
                self.status = why;
                return;
            }
        }
        let mut groups = self.groups.clone();
        let Some(gr) = groups.get_mut(g) else { return };
        gr.name = name.clone();
        self.commit(self.order.clone(), groups);
        self.status = format!("Renamed the group \"{name}\"");
    }

    /// Groups holding no samples at all (only empty slots, or none).
    pub(super) fn empty_groups(&self) -> usize {
        self.groups.iter().filter(|g| g.members.iter().all(|&x| self.free(x))).count()
    }

    /// Forgets every group with no samples in it. Slots don't move.
    pub(super) fn remove_empty_groups(&mut self) {
        let groups: Vec<Group> = self.groups.iter().filter(|g| !g.members.iter().all(|&x| self.free(x))).cloned().collect();
        let removed = self.groups.len() - groups.len();
        if removed == 0 {
            self.status = "No empty groups to remove".into();
            return;
        }
        self.commit(self.order.clone(), groups);
        self.status = format!("Removed {removed} empty group(s)");
    }

    /// Why group `g` can't be compacted, or how many empty slots it would drop.
    pub(super) fn compactable(&self, g: usize) -> Result<usize, &'static str> {
        let gr = self.groups.get(g).ok_or("No such group")?;
        if gr.members.len() < 2 {
            return Err("The group is a single slot");
        }
        let empty = gr.members.iter().filter(|&&x| self.free(x)).count();
        match empty {
            0 => Err("The group has no empty slots"),
            e if e == gr.members.len() => Ok(e - 1),
            e => Ok(e),
        }
    }

    /// Drops group `g`'s empty slots: its samples close up at its top (in
    /// order) and the freed slots stay right after it, ungrouped. An all-empty
    /// container shrinks to its first slot. Nothing outside the group moves.
    pub(super) fn compact_group(&mut self, g: usize) {
        if let Err(why) = self.compactable(g) {
            self.status = why.into();
            return;
        }
        let span = self.group_spans()[g].clone();
        let gr = &self.groups[g];
        let (filled, empty): (Vec<u16>, Vec<u16>) = self.order[span.clone()].iter().copied().partition(|&id| !self.free(id));
        let members = if filled.is_empty() { vec![self.order[span.start]] } else { filled.clone() };
        let dropped = gr.members.len() - members.len();
        let name = gr.name.clone();
        let mut order = self.order.clone();
        let mut run = filled;
        run.extend(empty);
        order.splice(span, run);
        let mut groups = self.groups.clone();
        groups[g].members = members;
        self.commit(order, groups);
        self.status = format!("Compacted \"{name}\": dropped {dropped} empty slot(s)");
    }

    /// Forgets group `g`; its samples stay where they are.
    pub(super) fn dissolve_group(&mut self, g: usize) {
        if g >= self.groups.len() {
            return;
        }
        let mut groups = self.groups.clone();
        let gone = groups.remove(g);
        self.commit(self.order.clone(), groups);
        self.status = format!("Dissolved the group \"{}\"; its samples stay where they are", gone.name);
    }

    pub(super) fn select_group(&mut self, g: usize) {
        let Some(span) = self.group_spans().get(g).cloned() else { return };
        self.sel = span.clone().collect();
        self.cursor = Some(span.start);
        self.group_held = Some(g);
    }

    /// Jumps to group `g` from pane `side`: the pane opens its bank and, when
    /// the group isn't all in view, eases (via `align[side]`) to show it from
    /// one row above its first slot. Its first slot flashes and the group is
    /// selected. (Not `ping_scroll`: the jump row draws after the list, and
    /// the pane clears that flag the same frame.)
    pub(super) fn go_to_group(&mut self, g: usize, side: usize) {
        let Some(span) = self.group_spans().get(g).cloned() else { return };
        self.select_group(g);
        self.active = side;
        self.ping = Some((span.start, std::time::Instant::now()));
        let bank = span.start / BANK;
        let rows = BANK.min(self.order.len() - bank * BANK);
        let (top, h) = (self.scroll[side], self.view_h[side]);
        let (a, b) = ((span.start % BANK) as f32 * ROW_H, ((span.end - 1) % BANK + 1) as f32 * ROW_H);
        if self.bank[side] != bank || a < top || b > top + h {
            self.bank[side] = bank;
            let max = (rows as f32 * ROW_H - h).max(0.0);
            self.align[side] = Some((a - ROW_H).clamp(0.0, max));
        }
    }

    /// True when the selection is exactly group `g`'s slots.
    pub(super) fn group_selected(&self, g: usize) -> bool {
        let Some(span) = self.group_spans().get(g).cloned() else { return false };
        self.sel.len() == span.len() && span.clone().all(|s| self.sel.contains(&s))
    }

    /// For every bank without a group named like `g`: where a one-slot
    /// empty container would go, `(bank, slot)`, or `None` when the bank has
    /// no empty, ungrouped slot. Prefers the slot number in the middle of
    /// `g`, then the nearest one.
    pub(super) fn spread_targets(&self, g: usize) -> Vec<(usize, Option<usize>)> {
        let spans = self.group_spans();
        let (Some(gr), Some(span)) = (self.groups.get(g), spans.get(g)) else { return Vec::new() };
        let name = gr.name.trim().to_lowercase();
        let n = self.order.len();
        let off = mid_of(span);
        let open = |p: usize| p >= 1 && self.free(self.order[p]) && self.group_at(p).is_none();
        (0..n.div_ceil(BANK))
            .filter(|&b| b != span.start / BANK)
            .filter(|&b| {
                !self.groups.iter().zip(&spans).any(|(h, r)| r.start / BANK == b && h.name.trim().to_lowercase() == name)
            })
            .map(|b| {
                let first = b * BANK;
                let last = (first + BANK).min(n);
                let at = (first..last).filter(|&a| open(a)).min_by_key(|&a| (a - first).abs_diff(off));
                (b, at)
            })
            .collect()
    }

    /// Makes a one-slot empty group named like `g` in every bank that has
    /// none, as one undo step.
    pub(super) fn spread_group(&mut self, g: usize) {
        let Some(gr) = self.groups.get(g) else { return };
        let name = gr.name.clone();
        let off = mid_of(&self.group_spans()[g]);
        let targets = self.spread_targets(g);
        let letter = |b: usize| (b'A' + b as u8) as char;
        if targets.is_empty() {
            self.status = format!("Every bank already has a \"{name}\" group");
            return;
        }
        let mut groups = self.groups.clone();
        let (mut same, mut moved, mut none) = (Vec::new(), Vec::new(), Vec::new());
        for &(b, at) in &targets {
            match at {
                Some(a) => {
                    groups.push(Group { name: name.clone(), members: vec![self.order[a]] });
                    if a % BANK == off { same.push(letter(b).to_string()) } else { moved.push(self.label(a)) }
                }
                None => none.push(letter(b).to_string()),
            }
        }
        let mut parts = Vec::new();
        if !same.is_empty() {
            parts.push(format!("in {} at {:03}", same.join(", "), off));
        }
        if !moved.is_empty() {
            parts.push(format!("at {}", moved.join(", ")));
        }
        let mut note = if parts.is_empty() {
            format!("No room for an empty \"{name}\"")
        } else {
            self.commit(self.order.clone(), groups);
            format!("Made empty \"{name}\" groups {}", parts.join("; "))
        };
        if !none.is_empty() {
            note += &format!(" · no empty slot in {}", none.join(", "));
        }
        self.status = note;
    }

    /// Moves group `g` toward where the same-named groups in the other banks
    /// keep their samples: the mean slot offset (within the bank) over their
    /// samples. `g` is placed so its middle lands on that mean, or on the
    /// nearest spot its bank allows. Slots in its bank move out of the way as
    /// for a group drop; a spot that would push anything into another bank is
    /// skipped.
    pub(super) fn plan_gather(&self, g: usize) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let n = self.order.len();
        let own = span.start / BANK;
        let want = gr.name.trim().to_lowercase();
        let letter = |b: usize| (b'A' + b as u8) as char;
        let (mut sum, mut count) = (0usize, 0usize);
        let mut banks: Vec<char> = Vec::new();
        for (h, other) in self.groups.iter().enumerate() {
            let b = spans[h].start / BANK;
            if h == g || b == own || other.name.trim().to_lowercase() != want {
                continue;
            }
            // Samples count; a container with none counts by its empty slots.
            let filled: Vec<usize> = spans[h].clone().filter(|&s| !self.free(self.order[s])).collect();
            let slots: Vec<usize> = if filled.is_empty() { spans[h].clone().collect() } else { filled };
            sum += slots.iter().map(|s| s % BANK).sum::<usize>();
            count += slots.len();
            banks.push(letter(b));
        }
        if count == 0 {
            return Err(format!("No other bank has a \"{}\" group", gr.name));
        }
        let mean = sum as f32 / count as f32;
        let len = span.len();
        let base = own * BANK;
        let bank_len = BANK.min(n - base);
        if len > bank_len {
            return Err("The group doesn't fit in its bank".into());
        }
        // Where the group's middle should be, as its first in-bank offset.
        let ideal = mean - (len as f32 - 1.0) / 2.0;
        let mut tops: Vec<usize> = (0..=bank_len - len).filter(|&t| base + t >= 1).collect();
        tops.sort_by(|&a, &b| (a as f32 - ideal).abs().total_cmp(&(b as f32 - ideal).abs()));
        let banks: String = banks.iter().map(char::to_string).collect::<Vec<_>>().join(", ");
        for t in tops {
            let top = base + t;
            let Ok(p) = self.plan_picked(SlotDrop::Group { g, top }, None) else { continue };
            if p.order.iter().zip(&self.order).enumerate().any(|(i, (new, old))| new != old && i / BANK != own) {
                continue;
            }
            if p.order == self.order {
                return Err(format!(
                    "\"{}\" already sits as close to the mean slot {:03} of the other banks as its bank allows",
                    gr.name,
                    mean.round() as usize
                ));
            }
            let mut note = format!(
                "\"{}\" to {}–{}, nearest the mean slot {:03} of the \"{}\" groups in {banks}",
                gr.name,
                self.label(top),
                self.label(top + len - 1),
                mean.round() as usize,
                gr.name
            );
            if !p.note.is_empty() {
                note += &format!(" · {}", p.note);
            }
            return Ok(Plan { note, ..p });
        }
        Err(format!("No room in its bank to move \"{}\" toward slot {:03}", gr.name, mean.round() as usize))
    }

    /// Group `g` was clicked by its frame in pane `side`: scroll the other
    /// pane so its group of the same name (in the bank it shows) sits level
    /// with this one. When the other pane can't scroll that far, this pane
    /// moves to meet it.
    pub(super) fn align_group(&mut self, g: usize, side: usize) {
        if self.scroll_linked {
            self.status = "The panes are scroll-linked: unlock the padlock between them to line groups up".into();
            return;
        }
        let spans = self.group_spans();
        let (Some(gr), Some(span)) = (self.groups.get(g), spans.get(g).cloned()) else { return };
        let name = gr.name.trim().to_lowercase();
        let other = 1 - side;
        let n = self.order.len();
        let bank_o = self.bank[other];
        let same: Vec<usize> = (0..self.groups.len())
            .filter(|&h| h != g && self.groups[h].name.trim().to_lowercase() == name)
            .collect();
        let letter = |b: usize| (b'A' + b as u8) as char;
        let Some(h) = same.iter().copied().find(|&h| spans[h].start / BANK == bank_o) else {
            let mut elsewhere: Vec<char> = same.iter().map(|&h| letter(spans[h].start / BANK)).collect();
            elsewhere.dedup();
            self.status = if elsewhere.is_empty() {
                format!("No other group named \"{}\" to line up with", gr.name)
            } else {
                let list: Vec<String> = elsewhere.iter().map(char::to_string).collect();
                format!("No \"{}\" group in bank {}; there's one in {}", gr.name, letter(bank_o), list.join(", "))
            };
            return;
        };
        let max = |b: usize, s: usize| ((BANK.min(n - b * BANK)) as f32 * ROW_H - self.view_h[s]).max(0.0);
        let top = |r: &Range<usize>| (r.start % BANK) as f32 * ROW_H;
        let (top_a, top_b) = (top(&span), top(&spans[h]));
        // Where the clicked group sits in its pane now; the other one goes there.
        let y = top_a - self.scroll[side];
        let o = (top_b - y).clamp(0.0, max(bank_o, other));
        let a = (top_a - top_b + o).clamp(0.0, max(span.start / BANK, side));
        if (o - self.scroll[other]).abs() > 0.5 {
            self.align[other] = Some(o);
        }
        if (a - self.scroll[side]).abs() > 0.5 {
            self.align[side] = Some(a);
        }
        self.status = format!(
            "Lined up \"{}\": {} ↔ {}",
            gr.name,
            self.label(span.start),
            self.label(spans[h].start)
        );
    }

    /// Works out a drop without doing it: `Err` says why it can't happen.
    pub(super) fn plan(&self, d: SlotDrop) -> Result<Plan, String> {
        if let SlotDrop::Split { g, top } = d {
            return self.plan_split(g, top);
        }
        if let SlotDrop::Group { g, top } = d {
            if let Some(h) = self.same_name_in_bank(g, top / BANK) {
                return self.plan_merge(g, h, top);
            }
        }
        self.plan_picked(d, None)
    }

    /// Another group named like `g` in bank `bank` (never `g` itself).
    fn same_name_in_bank(&self, g: usize, bank: usize) -> Option<usize> {
        let want = hue_key(&self.groups.get(g)?.name);
        let spans = self.group_spans();
        (0..self.groups.len()).find(|&i| i != g && spans[i].start / BANK == bank && hue_key(&self.groups[i].name) == want)
    }

    /// Group `g` dropped at `top` in a bank that already has its same-named
    /// group `h`: `g`'s slots join `h` (before it when dropped at or above
    /// its top, else at its end) and `g` ends. Two groups of one name never
    /// share a bank.
    fn plan_merge(&self, g: usize, h: usize, top: usize) -> Result<Plan, String> {
        let spans = self.group_spans();
        let pos = positions(&self.order);
        let before = if top <= spans[h].start {
            self.groups[h].members.iter().copied().min_by_key(|&x| pos[x as usize])
        } else {
            None
        };
        let picked: Vec<usize> = spans[g].clone().collect();
        let p = self.plan_join_at(h, &picked, before)?;
        let name = &self.groups[g].name;
        let bank = (b'A' + (spans[h].start / BANK) as u8) as char;
        let rest = p.note.replace(&format!(" · \"{name}\" ends (empty)"), "");
        Ok(Plan { note: format!("Merge \"{name}\" into its namesake in bank {bank} · {rest}"), ..p })
    }

    /// ⌥-drop of group `g` at `top`: the group's second half moves to a new
    /// group of the same name there, the first half stays. Refused when the
    /// bank at `top` already has a group of that name. The new group is a
    /// packed run, so the drop is planned like a `Shift` of those slots.
    fn plan_split(&self, g: usize, top: usize) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let len = gr.members.len();
        if len < 2 {
            return Err(format!("\"{}\" has one slot: nothing to split", gr.name));
        }
        if let Some(why) = self.name_taken(&gr.name, top / BANK, None) {
            return Err(why);
        }
        let pos = positions(&self.order);
        let mut members = gr.members.clone();
        members.sort_by_key(|&x| pos[x as usize]);
        let moved = members.split_off(len - len / 2);
        let picked: Vec<usize> = moved.iter().map(|&x| pos[x as usize]).collect();
        let Plan { order, mut groups, note } = self.plan_picked(SlotDrop::Shift(top), Some(&picked))?;
        groups.push(Group { name: gr.name.clone(), members: moved.clone() });
        self.check_groups(&order, &groups)?;
        Ok(Plan { order, groups, note: format!("Split \"{}\" · {note}", gr.name) })
    }

    /// Moves `pick` into group `g` at its end, as a drop there would (the
    /// slot menu's "Move to group"). Members already in `g` stay put.
    pub(super) fn plan_join(&self, g: usize, pick: &[usize]) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let n = self.order.len();
        let mut picked: Vec<usize> =
            pick.iter().copied().filter(|&s| s > 0 && s < n && !gr.members.contains(&self.order[s])).collect();
        picked.sort_unstable();
        picked.dedup();
        if picked.is_empty() {
            return Err(format!("Already all in \"{}\"", gr.name));
        }
        self.plan_picked(SlotDrop::Into { g, before: None, line: 0 }, Some(&picked))
    }

    /// Joins `pick` to group `g` (see `plan_join`) as one undo step.
    pub(super) fn join_group(&mut self, g: usize, pick: &[usize]) {
        match self.plan_join_in_bank(g, pick) {
            Ok(p) => {
                self.commit(p.order, p.groups);
                self.status = p.note;
            }
            Err(why) => self.status = why,
        }
    }

    /// `plan_join`, refused when the shuffle would move a slot into a bank
    /// other than the group's or the ones `pick` came from. Pushes that run
    /// over the bank's edge are caught here, not left to the next bank.
    pub(super) fn plan_join_in_bank(&self, g: usize, pick: &[usize]) -> Result<Plan, String> {
        self.plan_join_at(g, pick, None)
    }

    /// `plan_join_in_bank`, landing before member `before` (`None`: at the end).
    fn plan_join_at(&self, g: usize, pick: &[usize], before: Option<u16>) -> Result<Plan, String> {
        let plan = match before {
            None => self.plan_join(g, pick)?,
            Some(_) => self.plan_picked(SlotDrop::Into { g, before, line: 0 }, Some(pick))?,
        };
        let Some(span) = self.group_spans().get(g).cloned() else { return Err("No such group".into()) };
        let letter = |b: usize| (b'A' + b as u8) as char;
        let mut allowed: HashSet<usize> = pick.iter().filter(|&&s| s < self.order.len()).map(|&s| s / BANK).collect();
        allowed.insert(span.start / BANK);
        let spilled = plan
            .order
            .iter()
            .zip(&self.order)
            .enumerate()
            .filter(|(_, (new, old))| new != old)
            .map(|(i, _)| i / BANK)
            .find(|b| !allowed.contains(b));
        match spilled {
            Some(b) => Err(format!(
                "That would spill into bank {} (the group is in {}); the group's bank has no room for it",
                letter(b),
                letter(span.start / BANK)
            )),
            None => Ok(plan),
        }
    }

    /// Reorders group `g` within its bank so it sits at `at` among the bank's
    /// *other* groups in slot order (0: before the first; their count: after
    /// the last). Nothing leaves the bank. Every way allowed by `cap` is
    /// worked out and the one that disturbs least wins (see `Shuffle`):
    /// fewest other slots moved, then the shortest total distance they move,
    /// then the shortest trip for `g` itself (so it crawls toward the target
    /// and stops at the first good spot). Ties go to the gentler level.
    pub(super) fn plan_reorder(&self, g: usize, at: usize, cap: Shuffle) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let len = span.len();
        let n = self.order.len();
        let bank = span.start / BANK;
        let base = (bank * BANK).max(1);
        let end = bank_end(span.start, n);
        let mut others: Vec<usize> = (0..spans.len()).filter(|&h| h != g && spans[h].start / BANK == bank).collect();
        others.sort_by_key(|&h| spans[h].start);
        let at = at.min(others.len());
        let a = at.checked_sub(1).map(|i| others[i]);
        let b = others.get(at).copied();
        let name = |h: usize| format!("\"{}\"", self.groups[h].name);
        let place = match (a, b) {
            (Some(a), Some(b)) => format!("between {} and {}", name(a), name(b)),
            (Some(a), None) => format!("after {}", name(a)),
            (None, Some(b)) => format!("before {}", name(b)),
            (None, None) => return Err(format!("\"{}\" is the only group in its bank", gr.name)),
        };
        if others.iter().filter(|&&h| spans[h].start < span.start).count() == at {
            return Err(format!("\"{}\" is already {place}", gr.name));
        }
        let lo = a.map_or(base, |a| spans[a].end);
        let hi = b.map_or(end, |b| spans[b].start);
        let before = positions(&self.order);
        let mine: HashSet<u16> = gr.members.iter().copied().collect();
        // The spacing the user keeps between groups in this bank: the
        // smallest gap they left between two neighbouring groups (1 if they
        // all touch). The moved group tries to keep that much on each side.
        let bank_spans = |groups: &[Group], pos: &[usize]| -> Vec<Range<usize>> {
            let mut v: Vec<Range<usize>> = groups
                .iter()
                .map(|h| span_of(h, pos))
                .filter(|s| !s.is_empty() && s.start / BANK == bank)
                .collect();
            v.sort_by_key(|s| s.start);
            v
        };
        let pad = {
            let now = bank_spans(&self.groups, &before);
            now.windows(2).map(|w| w[1].start.saturating_sub(w[0].end)).filter(|&d| d > 0).min().unwrap_or(1)
        };
        // Is the result in the wanted order, inside the bank, and how much
        // did it disturb? `None`: not usable. Scored, lowest wins: groups
        // left touching (gaps between groups are only given up when there's
        // no room to keep them), slots moved, their distance, how far the
        // moved group falls short of `pad` on its sides, its own trip.
        let judge = |p: &Plan| -> Option<(usize, usize, usize, usize, usize)> {
            if p.groups.len() != self.groups.len() {
                return None;
            }
            if p.order.iter().zip(&self.order).enumerate().any(|(i, (x, y))| x != y && i / BANK != bank) {
                return None;
            }
            let pos = positions(&p.order);
            let new = |h: usize| span_of(&p.groups[h], &pos);
            let gs = new(g);
            if a.is_some_and(|a| new(a).end > gs.start) || b.is_some_and(|b| new(b).start < gs.end) {
                return None;
            }
            let grouped: HashSet<u16> = self.groups.iter().flat_map(|g| g.members.iter().copied()).collect();
            let (mut moved, mut dist) = (0, 0);
            for (i, &id) in p.order.iter().enumerate() {
                let was = before[id as usize];
                if was != i && !mine.contains(&id) && (!self.free(id) || grouped.contains(&id)) {
                    moved += 1;
                    dist += was.abs_diff(i);
                }
            }
            let touching = bank_spans(&p.groups, &pos).windows(2).filter(|w| w[1].start <= w[0].end).count();
            let short = a.map_or(0, |a| pad.saturating_sub(gs.start.saturating_sub(new(a).end)))
                + b.map_or(0, |b| pad.saturating_sub(new(b).start.saturating_sub(gs.end)));
            Some((touching, moved, dist, short, gs.start.abs_diff(span.start)))
        };
        type Score = (usize, usize, usize, usize, usize);
        let mut best: Option<(Score, Shuffle, Plan)> = None;
        let mut offer = |score: Score, level: Shuffle, p: Plan| {
            if level <= cap && best.as_ref().is_none_or(|(s, l, _)| (score, level) < (*s, *l)) {
                best = Some((score, level, p));
            }
        };
        // Fit / Push: lift the group out and drop it on each spot between
        // its new neighbours, pushing aside as a group drop does.
        let last = end.saturating_sub(len);
        for top in lo.min(last)..=hi.min(last) {
            let Ok(p) = self.plan_picked(SlotDrop::Group { g, top }, None) else { continue };
            let Some(score) = judge(&p) else { continue };
            let level = if score.1 == 0 { Shuffle::Fit } else { Shuffle::Push };
            offer(score, level, p);
        }
        // Rotate: the slots between the group and its new neighbour slide
        // over by its length, gaps and all; the group takes their place.
        // Always fits, since the bank's slot count doesn't change.
        if cap >= Shuffle::Rotate {
            let mut order = self.order.clone();
            if span.end <= lo {
                order[span.start..lo].rotate_left(len);
            } else {
                order[hi..span.end].rotate_right(len);
            }
            let p = Plan { order, groups: self.groups.clone(), note: String::new() };
            if self.check_groups(&p.order, &p.groups).is_ok() {
                if let Some(score) = judge(&p) {
                    offer(score, Shuffle::Rotate, p);
                }
            }
        }
        let Some(((_, moved, _, _, _), level, p)) = best else {
            return Err(match cap {
                Shuffle::Fit => format!("No free room {place} for \"{}\" ({len} slots); release ⇧ to push slots aside", gr.name),
                _ => format!("Can't move \"{}\" {place} without leaving its bank", gr.name),
            });
        };
        let land = span_of(&p.groups[g], &positions(&p.order)).start;
        let how = match level {
            Shuffle::Fit => "fits in free slots, nothing else moves".to_string(),
            Shuffle::Push => format!("{moved} slot(s) pushed into free room"),
            Shuffle::Rotate => format!("no free room: {moved} slot(s) slide over by {len}"),
        };
        let note = format!(
            "Move \"{}\" {place}, to {}–{} · {how}",
            gr.name,
            self.label(land),
            self.label(land + len - 1)
        );
        Ok(Plan { note, ..p })
    }

    /// Arrow keys on a selected group: slides it `step` slots (negative: up)
    /// inside its bank; the slots it passes over, empty or not, go to its
    /// other side. |step| > 1 means "up to": it stops short of a neighbour
    /// group or the bank's edge. A one-slot step into a neighbour group
    /// hops the whole neighbour. At the bank's edge it crosses into the next
    /// bank (`plan_cross`).
    pub(super) fn plan_step(&self, g: usize, step: i32) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let spans = self.group_spans();
        let span = spans[g].clone();
        let len = span.len();
        let n = self.order.len();
        let base = (span.start / BANK * BANK).max(1);
        let end = bank_end(span.start, n);
        let down = step > 0;
        let room = if down { end - span.end } else { span.start - base };
        if room == 0 {
            return self.plan_cross(g, down);
        }
        let want = (step.unsigned_abs() as usize).min(room);
        // The run slid over must not end inside another group.
        let edge = |k: usize| if down { span.end + k } else { span.start - k };
        let cut = spans.iter().enumerate().find(|&(h, r)| h != g && r.start < edge(want) && edge(want) < r.end).map(|(_, r)| r.clone());
        let k = match cut {
            None => want,
            Some(r) => {
                let (shrink, grow) = if down {
                    (r.start - span.end, r.end - span.end)
                } else {
                    (span.start - r.end, span.start - r.start)
                };
                if shrink > 0 && step.abs() > 1 { shrink } else { grow }
            }
        };
        let mut order = self.order.clone();
        if down {
            order[span.start..span.end + k].rotate_right(k);
        } else {
            order[span.start - k..span.end].rotate_left(k);
        }
        self.check_groups(&order, &self.groups)?;
        let land = if down { span.start + k } else { span.start - k };
        let passed = (if down { span.end..span.end + k } else { span.start - k..span.start })
            .filter(|&s| !self.free(self.order[s]))
            .count();
        let mut note = format!("Move \"{}\" to {}–{}", gr.name, self.label(land), self.label(land + len - 1));
        if passed > 0 {
            note += &format!(" · {passed} slot(s) moved {}", if down { "up" } else { "down" });
        }
        Ok(Plan { order, groups: self.groups.clone(), note })
    }

    /// `plan_step` past the bank's edge: group `g` lands at the near end of
    /// the next bank (top when going down, bottom when going up), that
    /// bank's slots shifting away into its free slots nearest the edge
    /// (loose empties, not group members). The empties it uses fill the
    /// group's old place. Refused when the bank lacks room or already has
    /// a group of that name.
    fn plan_cross(&self, g: usize, down: bool) -> Result<Plan, String> {
        let Some(gr) = self.groups.get(g) else { return Err("No such group".into()) };
        let span = self.group_spans()[g].clone();
        let len = span.len();
        let n = self.order.len();
        let bank = span.start / BANK;
        let letter = |b: usize| (b'A' + b as u8) as char;
        let tb = match (down, bank) {
            (false, 0) => return Err(format!("\"{}\" is at the top of bank A", gr.name)),
            (false, b) => b - 1,
            (true, b) => b + 1,
        };
        let (tbase, tend) = ((tb * BANK).max(1), ((tb + 1) * BANK).min(n));
        if tbase >= n {
            return Err(format!("\"{}\" is at the end of the last bank", gr.name));
        }
        if let Some(why) = self.name_taken(&gr.name, tb, Some(g)) {
            return Err(why);
        }
        let grouped: HashSet<u16> = self.groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        let mut cells: Vec<u16> = self.order[tbase..tend].to_vec();
        let mut loose: Vec<usize> = (0..cells.len()).filter(|&i| self.free(cells[i]) && !grouped.contains(&cells[i])).collect();
        if loose.len() < len {
            return Err(format!("Bank {} has {} free slot(s); \"{}\" needs {len}", letter(tb), loose.len(), gr.name));
        }
        if down {
            loose.truncate(len);
        } else {
            loose.drain(..loose.len() - len);
        }
        let pushed = if down { loose[len - 1] + 1 - len } else { cells.len() - loose[0] - len };
        let empties: Vec<u16> = loose.iter().map(|&i| cells[i]).collect();
        for &i in loose.iter().rev() {
            cells.remove(i);
        }
        let members = self.order[span.clone()].to_vec();
        let cells = if down { [members, cells].concat() } else { [cells, members].concat() };
        let mut order = self.order.clone();
        order[tbase..tend].copy_from_slice(&cells);
        order[span.clone()].copy_from_slice(&empties);
        self.check_groups(&order, &self.groups)?;
        let land = if down { tbase } else { tend - len };
        let mut note = format!("Move \"{}\" to bank {}, {}–{}", gr.name, letter(tb), self.label(land), self.label(land + len - 1));
        let shifted = (if down { tbase + len..tbase + len + pushed } else { tend - len - pushed..tend - len })
            .filter(|&s| !self.free(order[s]))
            .count();
        if shifted > 0 {
            note += &format!(" · {shifted} slot(s) shifted {}", if down { "down" } else { "up" });
        }
        Ok(Plan { order, groups: self.groups.clone(), note })
    }

    /// Applies `plan_step` and keeps the group selected and in view.
    pub(super) fn step_group(&mut self, g: usize, step: i32) {
        match self.plan_step(g, step) {
            Ok(p) => {
                self.commit(p.order, p.groups);
                self.select_group(g);
                if let Some(span) = self.group_spans().get(g).cloned() {
                    let lead = if step > 0 { span.end - 1 } else { span.start };
                    self.cursor = Some(lead);
                    self.reveal = Some(lead);
                }
                self.status = p.note;
            }
            Err(why) => self.status = why,
        }
    }

    /// `pick`: the slots to move instead of the selection.
    fn plan_picked(&self, d: SlotDrop, pick: Option<&[usize]>) -> Result<Plan, String> {
        let n = self.order.len();
        let spans = self.group_spans();
        let (picked, whole): (Vec<usize>, Option<usize>) = match d {
            SlotDrop::Group { g, .. } => (spans.get(g).ok_or("No such group")?.clone().collect(), Some(g)),
            _ => {
                let from: Vec<usize> = match pick {
                    Some(p) => p.to_vec(),
                    None => self.sel.iter().copied().collect(),
                };
                (from.into_iter().filter(|&s| s > 0 && s < n).collect(), None)
            }
        };
        if picked.is_empty() {
            return Err("Nothing to move".into());
        }
        let m = picked.len();
        if let SlotDrop::Place(top) = d {
            return self.plan_place(top, &picked, &spans);
        }
        let moved: Vec<u16> = picked.iter().map(|&s| self.order[s]).collect();
        let mut groups = self.groups.clone();
        let left: Vec<usize> = match whole {
            Some(_) => Vec::new(),
            None => groups.iter().enumerate().filter(|(_, g)| g.members.iter().any(|x| moved.contains(x))).map(|(i, _)| i).collect(),
        };
        // Lift the moved slots out. A group they leave closes up, pulling
        // what follows up as far as the next empty slot (in its bank).
        let mut cells: Vec<Option<u16>> = self.order.iter().copied().map(Some).collect();
        let mut closes = Vec::new();
        for &p in &picked {
            if whole.is_none() && left.iter().any(|&g| groups[g].members.contains(&self.order[p])) {
                closes.push(p);
            }
            cells[p] = None;
        }
        // Containers keep their size after losing members, if there's room.
        let sizes: Vec<Option<usize>> = groups
            .iter()
            .map(|g| g.members.iter().any(|&x| self.free(x)).then_some(g.members.len()))
            .collect();
        if whole.is_none() {
            for g in &mut groups {
                g.members.retain(|x| !moved.contains(x));
            }
        }
        let grouped: HashSet<u16> = groups.iter().flat_map(|g| g.members.iter().copied()).collect();
        // Free for a push: empty and in no group; dropping into a group may
        // also fill that group's own empty slots.
        let into = match d {
            SlotDrop::Into { g, .. } => Some(g),
            _ => None,
        };
        let loose = |id: u16| self.free(id) && !grouped.contains(&id);
        let absorbs = |id: u16| loose(id) || (self.free(id) && into.is_some_and(|g| groups[g].members.contains(&id)));
        for &p in closes.iter().rev() {
            let end = bank_end(p, n);
            let e = (p + 1..end).find(|&i| cells[i].is_none_or(loose)).unwrap_or(end);
            cells[p..e].rotate_left(1);
        }
        // Where the moved slots go, in the lifted-out layout.
        let at = match d {
            SlotDrop::Shift(top) | SlotDrop::Group { top, .. } => top.clamp(1, n - m),
            SlotDrop::Into { g, before, .. } => {
                let find = |id: u16| cells.iter().position(|c| *c == Some(id));
                match before {
                    Some(id) => find(id).ok_or("The slot to insert before moved")?,
                    None => match groups[g].members.iter().filter_map(|&x| find(x)).max() {
                        Some(last) => last + 1,
                        None => return Err("Drag the group by its name or frame to move all of it".into()),
                    },
                }
            }
            SlotDrop::Place(_) | SlotDrop::Split { .. } => unreachable!(),
        };
        // Put them in, pushing what's there down until enough empty slots
        // (including the ones just left) take up the shift. When there's no
        // room below (bank or pool end, or a group would be split), push
        // what's above the landing spot up instead.
        let place = |up: bool| -> Result<(Vec<u16>, Vec<Group>, usize), String> {
            let mut cells = cells.clone();
            let mut groups = groups.clone();
            let mut spare = Vec::new();
            let mut carry = std::collections::VecDeque::new();
            if up {
                // The block ends right before `at` (Into) or covers at..at+m.
                let end = if into.is_some() { at } else { at + m };
                let mut incoming = moved.iter().rev().copied();
                let mut i = end;
                loop {
                    let Some(next) = incoming.next().or_else(|| carry.pop_front()) else { break };
                    if i <= 1 {
                        return Err("No empty slot above to make room".into());
                    }
                    i -= 1;
                    match cells[i].replace(next) {
                        Some(old) if absorbs(old) => spare.push(old),
                        Some(old) => carry.push_back(old),
                        None => {}
                    }
                }
            } else {
                let mut incoming = moved.iter().copied();
                let mut i = at;
                loop {
                    let Some(next) = incoming.next().or_else(|| carry.pop_front()) else { break };
                    if i >= n {
                        return Err("No empty slot below to make room".into());
                    }
                    match cells[i].replace(next) {
                        Some(old) if absorbs(old) => spare.push(old),
                        Some(old) => carry.push_back(old),
                        None => {}
                    }
                    i += 1;
                }
            }
            let spare_ids: HashSet<u16> = spare.iter().copied().collect();
            let mut spare = spare.into_iter();
            for c in cells.iter_mut().filter(|c| c.is_none()) {
                *c = spare.next();
            }
            let order: Vec<u16> = cells.into_iter().map(|c| c.expect("every lifted slot refilled")).collect();
            let mut filled = 0;
            if let Some(g) = into {
                let before = groups[g].members.len();
                groups[g].members.retain(|x| !spare_ids.contains(x));
                filled = before - groups[g].members.len();
                groups[g].members.extend(&moved);
            }
            let pos = positions(&order);
            for g in &mut groups {
                g.members.sort_by_key(|x| pos[*x as usize]);
            }
            regrow(&mut groups, &sizes, &order, &pos, |id| self.free(id));
            self.check_groups(&order, &groups.iter().filter(|g| !g.members.is_empty()).cloned().collect::<Vec<_>>())?;
            Ok((order, groups, filled))
        };
        let (upward, (order, mut groups, filled)) = match place(false) {
            // Into a container: filling its empty slots above beats growing.
            Ok(r) if into.is_some() && r.2 < m => match place(true) {
                Ok(u) if u.2 > r.2 => (true, u),
                _ => (false, r),
            },
            Ok(r) => (false, r),
            Err(below) => match place(true) {
                Ok(r) => (true, r),
                Err(_) => return Err(below),
            },
        };
        let pos = positions(&order);
        let emptied: Vec<String> = groups.iter().filter(|g| g.members.is_empty()).map(|g| g.name.clone()).collect();
        groups.retain(|g| !g.members.is_empty());

        // What it says.
        let range = |a: usize| {
            if m == 1 { self.label(a) } else { format!("{}–{}", self.label(a), self.label(a + m - 1)) }
        };
        let before = positions(&self.order);
        let shifted: Vec<(usize, usize)> = (1..n)
            .map(|p| order[p])
            .filter(|&id| !moved.contains(&id) && !self.free(id))
            .map(|id| (before[id as usize], pos[id as usize]))
            .filter(|(a, b)| a != b)
            .collect();
        let down: Vec<usize> = shifted.iter().filter(|(a, b)| b > a).map(|&(_, b)| b).collect();
        let ups: Vec<usize> = shifted.iter().filter(|(a, b)| b < a).map(|&(_, b)| b).collect();
        let up = ups.len();
        let land = pos[moved[0] as usize];
        let mut note = match d {
            SlotDrop::Group { g, .. } => format!("Move group \"{}\" to {}", self.groups[g].name, range(land)),
            SlotDrop::Into { g, .. } => {
                let name = &self.groups[g].name;
                if moved.iter().all(|x| self.groups[g].members.contains(x)) {
                    format!("Reorder in \"{name}\" at {}", range(land))
                } else if filled > 0 {
                    format!("Fill {filled} empty slot(s) of \"{name}\" at {}", range(land))
                } else {
                    format!("Add to \"{name}\" at {}", range(land))
                }
            }
            _ => {
                if picked.iter().copied().eq(land..land + m) && shifted.is_empty() {
                    return Err(format!("Already at {}", range(land)));
                }
                format!("Move to {}", range(land))
            }
        };
        if let Some(&last) = down.iter().max() {
            note += &format!(" · {} pushed down to {}", down.len(), self.label(last));
        }
        if upward {
            if let Some(&top) = ups.iter().min() {
                note += &format!(" · no room below: {up} pushed up to {}", self.label(top));
            }
        } else if up > 0 {
            note += &format!(" · {up} pulled up");
        }
        if whole.is_none() && !matches!(d, SlotDrop::Into { .. }) {
            for &g in &left {
                note += &format!(" · leaves \"{}\"", self.groups[g].name);
            }
        }
        for name in emptied {
            note += &format!(" · \"{name}\" ends (empty)");
        }
        Ok(Plan { order, groups, note })
    }

    /// ⌥: swap the selection into `top…` in place. Groups don't take part.
    fn plan_place(&self, top: usize, picked: &[usize], spans: &[Range<usize>]) -> Result<Plan, String> {
        let n = self.order.len();
        let m = picked.len();
        let top = top.clamp(1, n - m);
        let hit = |s: usize| spans.iter().any(|r| r.contains(&s));
        if picked.iter().any(|&s| hit(s)) || (top..top + m).any(hit) {
            return Err("⌥ swap leaves groups alone: release ⌥ to move into or out of a group".into());
        }
        let mut order = self.order.clone();
        slot_list::place(&mut order, picked, top, 1);
        let range = if m == 1 { self.label(top) } else { format!("{}–{}", self.label(top), self.label(top + m - 1)) };
        if picked.iter().copied().eq(top..top + m) {
            return Err(format!("Already at {range}"));
        }
        let swaps = (top..top + m).filter(|t| !picked.contains(t) && !self.free(self.order[*t])).count();
        let note = if swaps > 0 {
            format!("Swap into {range} · {swaps} go to the freed slots")
        } else {
            format!("Place at {range}")
        };
        Ok(Plan { order, groups: self.groups.clone(), note })
    }

    /// Every group packed with samples, inside one bank.
    fn check_groups(&self, order: &[u16], groups: &[Group]) -> Result<(), String> {
        let pos = positions(order);
        for g in groups {
            let span = span_of(g, &pos);
            if span.len() != g.members.len() {
                return Err(format!("That would split the group \"{}\"", g.name));
            }
            if span.start == 0 || span.start / BANK != (span.end - 1) / BANK {
                return Err(format!("That would push the group \"{}\" past the end of its bank", g.name));
            }
        }
        Ok(())
    }

    pub(super) fn apply_drop(&mut self, d: SlotDrop) {
        match self.plan(d) {
            Ok(p) => {
                // A merge drops `g`, so find it again by a member.
                let held = match d {
                    SlotDrop::Group { g, .. } => self.groups.get(g).and_then(|gr| gr.members.first().copied()),
                    _ => None,
                };
                self.commit(p.order, p.groups);
                if let Some(g) = held.and_then(|id| self.groups.iter().position(|gr| gr.members.contains(&id))) {
                    self.select_group(g);
                }
                if let SlotDrop::Split { .. } = d {
                    self.select_group(self.groups.len() - 1);
                }
                self.status = p.note;
            }
            Err(why) => self.status = why,
        }
    }

    /// After an edit that doesn't know about groups: each keeps its longest
    /// packed run in one bank; members outside it leave.
    pub(super) fn repair_groups(&self, order: &[u16]) -> Vec<Group> {
        let pos = positions(order);
        self.groups
            .iter()
            .filter_map(|g| {
                let mut ps: Vec<usize> = g.members.iter().map(|&x| pos[x as usize]).collect();
                ps.sort_unstable();
                let mut best: Range<usize> = 0..0;
                let mut i = 0;
                while i < ps.len() {
                    let mut j = i + 1;
                    while j < ps.len() && ps[j] == ps[j - 1] + 1 && ps[j] / BANK == ps[i] / BANK {
                        j += 1;
                    }
                    if j - i > best.len() {
                        best = i..j;
                    }
                    i = j;
                }
                let members: Vec<u16> = ps[best].iter().filter(|&&p| p > 0).map(|&p| order[p]).collect();
                (!members.is_empty()).then(|| Group { name: g.name.clone(), members })
            })
            .collect()
    }

    /// The saved groups for this unit, from the app (on start and when
    /// another unit is shown).
    pub fn set_saved_groups(&mut self, unit: String, saved: Vec<PoolGroup>) {
        self.groups_unit = Some(unit);
        self.saved_groups = saved;
        self.match_groups();
    }

    /// Saved groups whose samples sit packed in one bank of the project get
    /// shown; the others stay saved (another project's).
    pub(super) fn match_groups(&mut self) {
        self.groups.clear();
        self.groups_from.clear();
        if self.project.is_none() {
            return;
        }
        let n = self.order.len();
        let mut by_key: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for p in 1..n {
            if let Some(k) = self.key_at(p) {
                by_key.entry(k).or_default().push(p);
            }
        }
        let mut taken: HashSet<usize> = HashSet::new();
        for (i, sg) in self.saved_groups.iter().enumerate() {
            let len = sg.keys.len();
            // Where the group could start: lined up on its first sample, or
            // where it was saved when it holds only empty slots.
            let starts: Vec<usize> = match sg.keys.iter().position(|&k| k != EMPTY_KEY) {
                Some(j) => by_key.get(&sg.keys[j]).map_or(Vec::new(), |v| v.iter().filter_map(|&p| p.checked_sub(j)).collect()),
                None => sg.at.map(|a| a as usize).into_iter().collect(),
            };
            let fits = |a: usize| {
                a >= 1
                    && len > 0
                    && a + len <= n
                    && a / BANK == (a + len - 1) / BANK
                    && sg.keys.iter().enumerate().all(|(j, &k)| {
                        let p = a + j;
                        !taken.contains(&p)
                            && if k == EMPTY_KEY { self.free(self.order[p]) } else { self.key_at(p) == Some(k) }
                    })
            };
            let Some(a) = starts.into_iter().find(|&a| fits(a)) else { continue };
            taken.extend(a..a + len);
            self.groups.push(Group { name: sg.name.clone(), members: self.order[a..a + len].to_vec() });
            self.groups_from.push(i);
        }
    }

    /// Writes the shown groups back into the saved list (by +Drive hash),
    /// replacing the saved ones they came from.
    pub(super) fn sync_saved_groups(&mut self) {
        if self.project.is_none() || self.groups_unit.is_none() {
            return;
        }
        let mut kept: Vec<PoolGroup> = self
            .saved_groups
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.groups_from.contains(i))
            .map(|(_, g)| g.clone())
            .collect();
        let start = kept.len();
        let spans = self.group_spans();
        kept.extend(self.groups.iter().zip(&spans).map(|(g, span)| PoolGroup {
            name: g.name.clone(),
            keys: g.members.iter().map(|&x| if self.free(x) { EMPTY_KEY } else { self.entry(x as usize).key() }).collect(),
            at: Some(span.start as u16),
        }));
        self.groups_from = (start..kept.len()).collect();
        if kept != self.saved_groups {
            self.saved_groups = kept;
            self.groups_dirty = true;
        }
    }
}

/// Slot number (within its bank) in the middle of a group's span.
fn mid_of(span: &Range<usize>) -> usize {
    (span.start + (span.len().max(1) - 1) / 2) % BANK
}

/// Saved key of an empty slot in a group.
const EMPTY_KEY: (u32, u32) = (0, 0);

/// Containers (`sizes[g]` set: they held an empty slot) that lost members
/// take free, ungrouped slots right after them, then right before, in their
/// bank, back up to their old size.
fn regrow(groups: &mut [Group], sizes: &[Option<usize>], order: &[u16], pos: &[usize], free: impl Fn(u16) -> bool) {
    let n = order.len();
    for g in 0..groups.len() {
        let Some(size) = sizes.get(g).copied().flatten() else { continue };
        if groups[g].members.is_empty() {
            continue;
        }
        while groups[g].members.len() < size {
            let span = span_of(&groups[g], pos);
            let bank = span.start / BANK;
            let open = |p: usize| {
                p >= 1 && p < n && p / BANK == bank && free(order[p]) && !groups.iter().any(|h| h.members.contains(&order[p]))
            };
            if open(span.end) {
                groups[g].members.push(order[span.end]);
            } else if span.start > 0 && open(span.start - 1) {
                groups[g].members.insert(0, order[span.start - 1]);
            } else {
                break;
            }
        }
    }
}

fn span_of(g: &Group, pos: &[usize]) -> Range<usize> {
    let ps = g.members.iter().map(|&x| pos[x as usize]);
    let a = ps.clone().min().unwrap_or(0);
    let b = ps.max().map_or(0, |b| b + 1);
    a..b
}
