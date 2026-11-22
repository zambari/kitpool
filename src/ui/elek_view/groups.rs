//! Sample groups: named runs of adjacent sample slots in one bank that move
//! as one unit. Only this app knows them (the device has no such thing);
//! they're saved per unit by +Drive hash (`state::PoolGroup`), so a project
//! with the same samples gets its groups back.
//!
//! Rules: a group holds samples only, always packed and inside one bank.
//! Dropping slots into a group adds them and pushes what follows down as far
//! as the next empty slot; dragging one out closes the group up and pulls up
//! what follows, as far as the next empty slot. Every slot move is worked
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

    /// Slots `pick` can become a group: adjacent, in one bank, all samples,
    /// none grouped yet. `Err` says why not.
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
        if pick.iter().any(|&s| self.free(self.order[s])) {
            return Err("Groups hold samples only: close the empty slots first");
        }
        if pick.iter().any(|&s| self.group_at(s).is_some()) {
            return Err("Some of these slots are already in a group");
        }
        Ok(())
    }

    pub(super) fn new_group(&mut self, pick: &[usize], name: String) {
        if let Err(why) = self.groupable(pick) {
            self.status = why.into();
            return;
        }
        let mut pick = pick.to_vec();
        pick.sort_unstable();
        let mut groups = self.groups.clone();
        groups.push(Group { name: name.clone(), members: pick.iter().map(|&s| self.order[s]).collect() });
        self.commit(self.order.clone(), groups);
        self.status = format!("Grouped {} slot(s) as \"{name}\"", pick.len());
    }

    pub(super) fn rename_group(&mut self, g: usize, name: String) {
        let mut groups = self.groups.clone();
        let Some(gr) = groups.get_mut(g) else { return };
        gr.name = name.clone();
        self.commit(self.order.clone(), groups);
        self.status = format!("Renamed the group \"{name}\"");
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

    /// Works out a drop without doing it: `Err` says why it can't happen.
    pub(super) fn plan(&self, d: SlotDrop) -> Result<Plan, String> {
        let n = self.order.len();
        let spans = self.group_spans();
        let (picked, whole): (Vec<usize>, Option<usize>) = match d {
            SlotDrop::Group { g, .. } => (spans.get(g).ok_or("No such group")?.clone().collect(), Some(g)),
            _ => (self.sel.iter().copied().filter(|&s| s > 0 && s < n).collect(), None),
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
        if whole.is_none() {
            for g in &mut groups {
                g.members.retain(|x| !moved.contains(x));
            }
        }
        for &p in closes.iter().rev() {
            let end = bank_end(p, n);
            let e = (p + 1..end).find(|&i| cells[i].is_none_or(|id| self.free(id))).unwrap_or(end);
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
        // (including the ones just left) take up the shift.
        let mut spare = Vec::new();
        let mut carry = std::collections::VecDeque::new();
        let mut incoming = moved.iter().copied();
        let mut i = at;
        loop {
            let Some(next) = incoming.next().or_else(|| carry.pop_front()) else { break };
            if i >= n {
                return Err("No empty slot below to make room".into());
            }
            match cells[i].replace(next) {
                Some(old) if self.free(old) => spare.push(old),
                Some(old) => carry.push_back(old),
                None => {}
            }
            i += 1;
        }
        let mut spare = spare.into_iter();
        for c in cells.iter_mut().filter(|c| c.is_none()) {
            *c = spare.next();
        }
        let order: Vec<u16> = cells.into_iter().map(|c| c.expect("every lifted slot refilled")).collect();
        if let SlotDrop::Into { g, .. } = d {
            groups[g].members.extend(&moved);
        }
        let pos = positions(&order);
        for g in &mut groups {
            g.members.sort_by_key(|x| pos[*x as usize]);
        }
        let emptied: Vec<String> = groups.iter().filter(|g| g.members.is_empty()).map(|g| g.name.clone()).collect();
        groups.retain(|g| !g.members.is_empty());
        self.check_groups(&order, &groups)?;

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
        let up = shifted.iter().filter(|(a, b)| b < a).count();
        let land = pos[moved[0] as usize];
        let mut note = match d {
            SlotDrop::Group { g, .. } => format!("Move group \"{}\" to {}", self.groups[g].name, range(land)),
            SlotDrop::Into { g, .. } => {
                let name = &self.groups[g].name;
                if moved.iter().all(|x| self.groups[g].members.contains(x)) {
                    format!("Reorder in \"{name}\" at {}", range(land))
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
        if up > 0 {
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
            if g.members.iter().any(|&x| self.free(x)) {
                return Err(format!("That would leave an empty slot in the group \"{}\"", g.name));
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
                let mut ps: Vec<usize> = g.members.iter().filter(|&&x| !self.free(x)).map(|&x| pos[x as usize]).collect();
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

    /// Members just emptied (cleared) leave their group, which closes up,
    /// pulling up what follows as far as the next empty slot. Part of the
    /// clear's undo step.
    pub(super) fn close_freed_members(&mut self) {
        let pos = positions(&self.order);
        let mut freed: Vec<usize> = self
            .groups
            .iter()
            .flat_map(|g| g.members.iter().copied())
            .filter(|&x| self.free(x))
            .map(|x| pos[x as usize])
            .collect();
        if freed.is_empty() {
            return;
        }
        freed.sort_unstable();
        let n = self.order.len();
        let mut order = self.order.clone();
        for &p in freed.iter().rev() {
            let end = bank_end(p, n);
            let e = (p + 1..end).find(|&i| self.free(order[i])).unwrap_or(end);
            order[p..e].rotate_left(1);
        }
        let mut groups = self.groups.clone();
        for g in &mut groups {
            g.members.retain(|&x| !self.free(x));
        }
        groups.retain(|g| !g.members.is_empty());
        self.order = order;
        self.groups = groups;
        self.sync_saved_groups();
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
            let mut ps: Vec<usize> = Vec::new();
            for k in &sg.keys {
                match by_key.get(k).and_then(|v| v.iter().copied().find(|p| !taken.contains(p) && !ps.contains(p))) {
                    Some(p) => ps.push(p),
                    None => break,
                }
            }
            if ps.len() != sg.keys.len() || ps.is_empty() {
                continue;
            }
            let (a, b) = (*ps.iter().min().unwrap_or(&0), *ps.iter().max().unwrap_or(&0));
            if b - a + 1 != ps.len() || a / BANK != b / BANK {
                continue;
            }
            ps.sort_unstable();
            taken.extend(&ps);
            self.groups.push(Group { name: sg.name.clone(), members: ps.iter().map(|&p| self.order[p]).collect() });
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
        kept.extend(self.groups.iter().map(|g| PoolGroup {
            name: g.name.clone(),
            keys: g.members.iter().map(|&x| self.entry(x as usize).key()).collect(),
        }));
        self.groups_from = (start..kept.len()).collect();
        if kept != self.saved_groups {
            self.saved_groups = kept;
            self.groups_dirty = true;
        }
    }
}

fn span_of(g: &Group, pos: &[usize]) -> Range<usize> {
    let ps = g.members.iter().map(|&x| pos[x as usize]);
    let a = ps.clone().min().unwrap_or(0);
    let b = ps.max().map_or(0, |b| b + 1);
    a..b
}
