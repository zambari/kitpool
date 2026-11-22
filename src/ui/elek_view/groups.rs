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
    /// ⌥: the selection swaps in place.
    Place(usize),
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

fn bank_end(p: usize, n: usize) -> usize {
    ((p / BANK + 1) * BANK).min(n)
}

impl ElekState {
    /// Each group's slots now, by index.
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
        let mut groups = self.groups.clone();
        groups.push(Group { name: name.clone(), members: pick.iter().map(|&s| self.order[s]).collect() });
        self.commit(self.order.clone(), groups);
        self.status = format!("Grouped {} slot(s) as \"{name}\"", pick.len());
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

    /// Group `g` was clicked by its frame in pane `side`: scroll the other
    /// pane so its group of the same name (in the bank it shows) sits level
    /// with this one. When the other pane can't scroll that far, this pane
    /// moves to meet it.
    pub(super) fn align_group(&mut self, g: usize, side: usize) {
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
        self.plan_picked(d, None)
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
        match self.plan_join(g, pick) {
            Ok(p) => {
                self.commit(p.order, p.groups);
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
            SlotDrop::Place(_) => unreachable!(),
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
                self.commit(p.order, p.groups);
                if let SlotDrop::Group { g, .. } = d {
                    self.select_group(g);
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
